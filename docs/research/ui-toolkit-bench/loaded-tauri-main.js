// CodeMirror 6 SQL editor plus a hand-written virtual grid that asks Rust for the visible rows only.
import { basicSetup, EditorView } from "codemirror";
import { sql } from "@codemirror/lang-sql";

const { invoke } = window.__TAURI__.core;
const ROW_HEIGHT = 22;

new EditorView({
  doc: "SELECT o.id, o.created_at, c.name\nFROM orders AS o\nJOIN customers AS c ON c.id = o.customer_id\nWHERE o.created_at > '2026-01-01'\nORDER BY o.created_at DESC;",
  extensions: [basicSetup, sql()],
  parent: document.getElementById("editor"),
});

const grid = document.getElementById("grid");
const spacer = document.getElementById("spacer");
const rowsEl = document.getElementById("rows");

async function renderVisible() {
  const first = Math.floor(grid.scrollTop / ROW_HEIGHT);
  const count = Math.ceil(grid.clientHeight / ROW_HEIGHT) + 1;
  const rows = await invoke("rows", { start: first, count });
  rowsEl.style.top = `${first * ROW_HEIGHT}px`;
  rowsEl.replaceChildren(
    ...rows.map((cells) => {
      const tr = document.createElement("div");
      tr.className = "r";
      for (const text of cells) {
        const td = document.createElement("span");
        td.textContent = text;
        tr.append(td);
      }
      return tr;
    }),
  );
}

const total = await invoke("row_count");
spacer.style.height = `${total * ROW_HEIGHT}px`;
grid.addEventListener("scroll", () => requestAnimationFrame(renderVisible));
await renderVisible();

// Report the first painted frame with editor and grid (double rAF: after the next paint).
requestAnimationFrame(() => requestAnimationFrame(() => invoke("first_frame")));
