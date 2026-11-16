//! PROTOTYPE. SQL completion from a fixed keyword, table and column list, standing in for
//! the schema the engine would load from a real connection.

use anyhow::Result;
use gpui::{App, Task, Window};
use gpui_component::input::{CompletionProvider, Rope};
use lsp_types::{CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse};

const KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "JOIN", "INNER", "LEFT", "RIGHT", "OUTER", "ON", "GROUP", "BY",
    "ORDER", "HAVING", "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE", "TOP", "DISTINCT",
    "AS", "AND", "OR", "NOT", "NULL", "IS", "IN", "EXISTS", "BETWEEN", "LIKE", "CASE", "WHEN",
    "THEN", "ELSE", "END", "WITH", "UNION", "ALL", "COUNT", "SUM", "AVG", "MIN", "MAX",
];

const TABLES: &[(&str, &[&str])] = &[
    ("orders", &["order_id", "customer_id", "created_at", "status", "amount", "currency"]),
    ("customers", &["customer_id", "name", "email", "region_id", "created_at"]),
    ("regions", &["region_id", "name", "country"]),
    ("order_lines", &["order_id", "line_no", "sku", "quantity", "unit_price"]),
    ("warehouses", &["warehouse_id", "name", "region_id"]),
];

pub struct SqlCompletion;

/// The identifier being typed: the run of word characters ending at `offset`.
fn word_before(text: &Rope, offset: usize) -> String {
    let head = text.slice(0..offset).to_string();
    let start = head
        .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
        .map_or(0, |i| i + 1);
    head[start..].to_string()
}

impl CompletionProvider for SqlCompletion {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _trigger: CompletionContext,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        let word = word_before(text, offset).to_lowercase();
        let mut items = Vec::new();
        let mut push = |label: String, kind, detail: &str| {
            if label.to_lowercase().starts_with(&word) {
                items.push(CompletionItem {
                    label,
                    kind: Some(kind),
                    detail: Some(detail.to_string()),
                    ..Default::default()
                });
            }
        };
        for keyword in KEYWORDS {
            push(keyword.to_string(), CompletionItemKind::KEYWORD, "keyword");
        }
        for (table, columns) in TABLES {
            push(table.to_string(), CompletionItemKind::CLASS, "table");
            for column in *columns {
                push(column.to_string(), CompletionItemKind::FIELD, table);
            }
        }
        Task::ready(Ok(CompletionResponse::Array(items)))
    }

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _cx: &mut App) -> bool {
        new_text.chars().all(|c| c.is_alphanumeric() || c == '_') && !new_text.is_empty()
    }
}
