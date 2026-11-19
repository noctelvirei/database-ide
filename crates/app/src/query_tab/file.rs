//! What ties a query tab to the world outside its session (docs/spec.md, "Files, restored
//! tabs and history"): its `.sql` file, the form it is kept in across restarts, connecting a
//! restored tab on its first run, and the history entry each run leaves.

use std::path::{Path, PathBuf};

use dbide_engine::session::{Event, PlanMode};
use gpui::{App, Context, EventEmitter, SharedString, Window};

use super::{QueryTab, QueryTarget, Status};
use crate::history::{self, Entry, Outcome};
use crate::messages::Tone;
use crate::open_tabs::SavedTab;
use crate::runtime::{Job, Runtime};
use crate::sql_file::{self, Encoding};

/// The name the save prompt suggests for a tab with no file.
const SUGGESTED_NAME: &str = "Query.sql";

/// The `.sql` file a tab is tied to.
pub struct TabFile {
    pub path: PathBuf,
    pub encoding: Encoding,
    /// The file's text when the tab last read or wrote it; `None` when it couldn't be read,
    /// so the tab reads as changed.
    saved: Option<String>,
}

impl TabFile {
    /// The file at `path`, read now to know whether the tab's text differs from it.
    pub fn reopen(path: PathBuf, encoding: Encoding) -> Self {
        let saved = sql_file::read(&path).ok().map(|(text, _)| text);
        Self {
            path,
            encoding,
            saved,
        }
    }
}

/// What a tab asks of the workbench.
pub enum QueryTabEvent {
    /// A restored tab's first run needs the saved connection with this id signed in; answer
    /// with `QueryTab::resume`.
    SignIn(String),
    /// A run ended; the entry goes to the query history.
    Ran(Entry),
}

impl EventEmitter<QueryTabEvent> for QueryTab {}

impl QueryTab {
    /// A tab restored from `tabs.json` or reopened from the history: `text` in the editor,
    /// tied to `file`, on `target` without connecting until its first run (F5).
    pub fn restored(
        title: SharedString,
        target: Option<QueryTarget>,
        runtime: Runtime,
        text: &str,
        file: Option<TabFile>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let tab = Self::unconnected(title, target, runtime, window, cx);
        tab.editor.update(cx, |editor, cx| {
            editor.set_value(text.to_owned(), window, cx)
        });
        Self { file, ..tab }
    }

    /// The tab as `tabs.json` keeps it.
    pub fn saved(&self, cx: &App) -> SavedTab {
        SavedTab {
            text: self.editor.read(cx).value().to_string(),
            file: self.file.as_ref().map(|f| f.path.clone()),
            encoding: self.file.as_ref().map(|f| f.encoding).unwrap_or_default(),
            connection_id: self.target.as_ref().map(|t| t.saved.id.clone()),
            database: self.database.clone().or_else(|| {
                self.target
                    .as_ref()
                    .and_then(|t| t.saved.connection.database.clone())
            }),
        }
    }

    /// The file the tab is tied to, if any.
    pub fn file_path(&self) -> Option<&Path> {
        self.file.as_ref().map(|f| f.path.as_path())
    }

    /// True when the tab has a file and its text differs from what the file holds.
    pub fn is_modified(&self, cx: &App) -> bool {
        self.file
            .as_ref()
            .is_some_and(|f| f.saved.as_deref() != Some(&*self.editor.read(cx).value()))
    }

    /// The name the tab list shows: the file's name, or the tab's title without one.
    pub(super) fn name(&self) -> SharedString {
        self.file
            .as_ref()
            .and_then(|f| f.path.file_name())
            .map_or_else(
                || self.title.clone(),
                |name| name.to_string_lossy().to_string().into(),
            )
    }

    /// Ctrl+O on a `.sql` file: puts `text` in the editor, tied to `path`.
    pub fn load_file(
        &mut self,
        path: PathBuf,
        text: String,
        encoding: Encoding,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor
            .update(cx, |editor, cx| editor.set_value(text.clone(), window, cx));
        self.say(format!("Opened {}.", path.display()), Tone::Plain, cx);
        self.file = Some(TabFile {
            path,
            encoding,
            saved: Some(text),
        });
        cx.notify();
    }

    /// Ctrl+S: writes the text to the tab's file in its encoding, or asks where first for a
    /// tab with none (a new file is UTF-8 without a byte-order mark).
    pub fn save_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(file) = &self.file {
            let (path, encoding) = (file.path.clone(), file.encoding);
            self.write_file(path, encoding, cx);
            return;
        }
        let asked = crate::file_prompt::save_path(cx, SUGGESTED_NAME);
        self.choosing_file = true;
        cx.spawn_in(window, async move |this, cx| {
            let path = asked.await;
            let _ = this.update(cx, |this, cx| {
                this.choosing_file = false;
                if let Some(path) = path {
                    this.write_file(path, Encoding::default(), cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Writes the text to `path`; a `.sql` file is small, so on the UI thread.
    fn write_file(&mut self, path: PathBuf, encoding: Encoding, cx: &mut Context<Self>) {
        let text = self.editor.read(cx).value().to_string();
        match sql_file::write(&path, &text, encoding) {
            Ok(()) => {
                self.say(format!("Saved {}.", path.display()), Tone::Plain, cx);
                self.file = Some(TabFile {
                    path,
                    encoding,
                    saved: Some(text),
                });
            }
            Err(error) => self.say(
                format!("Couldn't save {}: {error}", path.display()),
                Tone::Error,
                cx,
            ),
        }
        cx.notify();
    }

    /// True for a restored tab that keeps its connection but hasn't connected yet.
    pub fn is_asleep(&self) -> bool {
        self.status == Status::Unconnected && self.target.is_some()
    }

    /// F5 on a restored tab: asks the workbench to sign in, then runs once connected.
    /// Asking again replaces an earlier request the user may have cancelled.
    pub(super) fn wake(&mut self, cx: &mut Context<Self>) {
        let Some(target) = &self.target else { return };
        let (id, name) = (
            target.saved.id.clone(),
            target.saved.connection.name.clone(),
        );
        self.run_when_connected = true;
        self.say(format!("Signing in to {name}…"), Tone::Plain, cx);
        cx.emit(QueryTabEvent::SignIn(id));
    }

    /// Another tab asked to sign in after this one, so this one's sign-in was cancelled:
    /// its F5 no longer runs once connected.
    pub fn stop_waiting(&mut self) {
        self.run_when_connected = false;
    }

    /// The workbench signed in for this tab: connects on `target`, keeping the tab's
    /// database, and runs. Does nothing unless the tab is still waiting for its F5.
    pub fn resume(&mut self, mut target: QueryTarget, window: &mut Window, cx: &mut Context<Self>) {
        if !self.run_when_connected {
            return;
        }
        if !self.is_asleep() {
            return;
        }
        target.saved.connection.database = self.saved(cx).database;
        self.target = Some(target);
        self.connect(window, cx);
    }

    /// Starts the history entry for a run of `batch` as `job`; an estimated plan, which
    /// runs nothing, leaves none.
    pub(super) fn start_entry(&mut self, batch: &str, job: &Job) {
        if matches!(job, Job::Plan(PlanMode::Estimated)) {
            self.entry = None;
            return;
        }
        self.entry = Some(Entry {
            started_ms: history::now_ms(),
            duration_ms: 0,
            connection_id: self.target.as_ref().map(|t| t.saved.id.clone()),
            connection: self
                .target
                .as_ref()
                .map(|t| t.saved.connection.name.clone()),
            database: self.database.clone(),
            text: batch.to_owned(),
            outcome: Outcome::Rows { rows: 0 },
        });
    }

    /// Notes what `event` says about the run's outcome.
    pub(super) fn note_event(&mut self, event: &Event) {
        let Some(entry) = &mut self.entry else { return };
        match (event, &mut entry.outcome) {
            (Event::RowCount(count), Outcome::Rows { rows }) => *rows += count,
            (Event::Error { text, .. }, Outcome::Rows { .. }) => {
                entry.outcome = Outcome::Error { text: text.clone() };
            }
            (Event::Done { elapsed }, _) => entry.duration_ms = elapsed.as_millis() as u64,
            _ => {}
        }
    }

    /// Notes a failure the app met on the run's behalf, such as writing its rows to a file,
    /// unless the server already reported one.
    pub(super) fn note_failure(&mut self, text: String) {
        if let Some(entry) = &mut self.entry
            && matches!(entry.outcome, Outcome::Rows { .. })
        {
            entry.outcome = Outcome::Error { text };
        }
    }

    /// The run ended: hands its entry to the workbench for the history.
    pub(super) fn finish_entry(&mut self, cx: &mut Context<Self>) {
        let Some(mut entry) = self.entry.take() else {
            return;
        };
        if self.run_cancelled {
            entry.outcome = Outcome::Cancelled;
        }
        cx.emit(QueryTabEvent::Ran(entry));
    }
}
