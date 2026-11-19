//! One query tab (docs/spec.md, "Runs and results"): an SQL editor bound to a connection
//! and its current database, with its own session, so tabs run and switch databases
//! independently, as SSMS's query windows do.
//!
//! The tab's session is in one of three places: here and free (`Ready`), on the runtime
//! doing work (`Connecting`, `Running`, `Switching`, `Listing`), or gone (`Unconnected`,
//! `Failed`). Each action checks that state first and says in the messages strip why it
//! can't go ahead. The current database is read from the session every time it comes back,
//! so a `USE` typed in the editor updates the tab too.

use std::fmt;

use dbide_engine::connections::SavedConnection;
use dbide_engine::danger::{self, DangerCheck, Engine};
use dbide_engine::plan::Plan;
use dbide_engine::session::{CancelHandle, Event, PlanMode, Session, Severity};
use futures::StreamExt as _;
use gpui::{App, AppContext as _, Context, Entity, SharedString, Window};
use gpui_component::input::{EditorState, Position};
use serde::Serialize;

use crate::messages::{Message, Messages, Tone};
use crate::results::{Results, ResultsEvent};
use crate::runtime::{Job, RunUpdate, Runtime};

pub mod dialogs;
pub mod file;

use dialogs::DialogState;

/// A password held in memory for the tab's reconnects; never shown or logged.
#[derive(Clone)]
pub struct HeldPassword(String);

impl HeldPassword {
    pub fn new(password: String) -> Self {
        Self(password)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for HeldPassword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HeldPassword([redacted])")
    }
}

/// The connection a tab opens, with the settings it inherits from its folders.
#[derive(Clone, Debug)]
pub struct QueryTarget {
    pub saved: SavedConnection,
    /// The colour's palette value, after folder inheritance.
    pub colour: Option<String>,
    /// Whether runs ask before dangerous statements, after folder inheritance.
    pub danger_check: bool,
    /// The password used to connect, kept for PostgreSQL's reconnect on a database switch.
    pub password: Option<HeldPassword>,
}

/// Where the tab's session is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// The tab has no connection.
    Unconnected,
    Connecting,
    /// Connected and free.
    Ready,
    Running,
    /// Switching to another database.
    Switching,
    /// Loading the database list for the picker.
    Listing,
    /// The connection failed or the engine stopped; the messages say why.
    Failed,
}

/// What a user can see of one tab.
#[derive(Debug, Serialize)]
pub struct TabState {
    pub title: SharedString,
    pub connection: Option<String>,
    pub database: Option<String>,
    pub colour: Option<&'static str>,
    pub status: Status,
    /// True when F5 returns the actual plan with the run (Ctrl+M).
    pub actual_plan: bool,
    /// True when F5 sends the rows to a file instead of the grid (Ctrl+Shift+F).
    pub results_to_file: bool,
    /// The rows a run to file has written so far, while it runs.
    pub rows_written: Option<u64>,
    /// The editor caret's line, counted from one.
    pub cursor_line: u32,
    /// The editor's find panel, while it is open.
    pub search: Option<SearchState>,
    /// The `.sql` file the tab is tied to.
    pub file: Option<std::path::PathBuf>,
    /// True while the text differs from the file.
    pub modified: bool,
    /// The editor's text.
    pub text: String,
}

/// What a user can see of the editor's find panel (Ctrl+F) or find and replace (Ctrl+H).
#[derive(Debug, Serialize)]
pub struct SearchState {
    pub replace: bool,
    pub query: String,
    /// The current match and the total, such as `1/3`; `0/0` with none.
    pub matches: String,
}

pub struct QueryTab {
    title: SharedString,
    target: Option<QueryTarget>,
    editor: Entity<EditorState>,
    runtime: Runtime,
    status: Status,
    /// The session while it is here and free; `None` while it is away or gone.
    session: Option<Box<dyn Session>>,
    database: Option<String>,
    /// The running batch's cancel handle, once the runtime has started it.
    cancel: Option<CancelHandle>,
    /// Cancel was pressed during the current run; a run not yet started is cancelled as it
    /// starts.
    cancel_pressed: bool,
    /// The engine confirmed the current run was cancelled, so its end says so. A cancel
    /// pressed after the run finished never confirms, and the run reads as finished.
    run_cancelled: bool,
    /// Counts the tab's runs, so a task started for one run stops when the next begins.
    runs: u64,
    /// True while the formatter works on the tab's text.
    formatting: bool,
    /// What the current or last run asked of the server.
    job: Job,
    /// Ctrl+M: F5 returns the actual plan with the run.
    include_actual_plan: bool,
    /// Ctrl+Shift+F: F5 asks for a file and sends the run's rows there, not to the grid.
    results_to_file: bool,
    /// True while the save prompt for a run to file is open.
    choosing_file: bool,
    /// False while another tab is on screen: a database list that loads then is dropped,
    /// so its picker never opens over another tab's editor.
    shown: bool,
    dialog: Option<dialogs::OpenDialog>,
    messages: Messages,
    /// The last run's result sets.
    results: Entity<Results>,
    /// The `.sql` file the tab is tied to (Ctrl+O, Ctrl+S).
    file: Option<file::TabFile>,
    /// The current run's history entry, while it runs.
    entry: Option<crate::history::Entry>,
    /// F5 on a restored tab signs in first; run once connected.
    run_when_connected: bool,
}

/// How often a running tab redraws its grids' row counts while rows arrive.
const ROW_COUNT_REFRESH: std::time::Duration = std::time::Duration::from_millis(250);

/// The text the engine gives the error that confirms a cancel (`dbide_engine::session::Event`).
const CANCELLED: &str = "Run was cancelled";

impl QueryTab {
    /// A tab titled `title`, connecting at once when it has a target.
    pub fn new(
        title: SharedString,
        target: Option<QueryTarget>,
        runtime: Runtime,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut tab = Self::unconnected(title, target, runtime, window, cx);
        tab.connect(window, cx);
        tab
    }

    /// A tab titled `title` on `target`, not connected.
    fn unconnected(
        title: SharedString,
        target: Option<QueryTarget>,
        runtime: Runtime,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("sql")
                .searchable(true)
        });
        let results = cx.new(Results::new);
        cx.subscribe(&results, |this: &mut Self, _, event: &ResultsEvent, cx| {
            let ResultsEvent::Message(text) = event;
            this.say(text.clone(), Tone::Plain, cx);
        })
        .detach();
        Self {
            database: target
                .as_ref()
                .and_then(|t| t.saved.connection.database.clone()),
            title,
            target,
            editor,
            runtime,
            status: Status::Unconnected,
            session: None,
            cancel: None,
            cancel_pressed: false,
            run_cancelled: false,
            runs: 0,
            formatting: false,
            job: Job::Run,
            include_actual_plan: false,
            results_to_file: false,
            choosing_file: false,
            shown: true,
            dialog: None,
            messages: Messages::default(),
            results,
            file: None,
            entry: None,
            run_when_connected: false,
        }
    }

    pub fn editor(&self) -> &Entity<EditorState> {
        &self.editor
    }

    pub fn target(&self) -> Option<&QueryTarget> {
        self.target.as_ref()
    }

    /// True while the tab waits on the server, the formatter, a file prompt, or a copy or
    /// save from its results.
    pub fn is_busy(&self, cx: &gpui::App) -> bool {
        matches!(
            self.status,
            Status::Connecting | Status::Running | Status::Switching | Status::Listing
        ) || self.formatting
            || self.choosing_file
            || self.results.read(cx).is_working()
    }

    /// Ctrl+K Ctrl+F: formats the selection, or the whole tab, in the connection's dialect,
    /// as one undo step (docs/spec.md, "Formatter"). The formatter works off the UI thread;
    /// if the text changes meanwhile, the result is dropped. When it refuses, the text stays
    /// as it was and the messages strip says why.
    pub fn format(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(engine) = self.target.as_ref().map(|t| t.saved.connection.engine) else {
            self.say(
                "Formatting follows the connection's SQL dialect, and this tab has no connection.",
                Tone::Plain,
                cx,
            );
            return;
        };
        if self.formatting {
            return;
        }
        let editor = self.editor.read(cx);
        let whole = editor.value();
        let selected = editor.selected_range();
        let part = if selected.is_empty() {
            whole.to_string()
        } else {
            whole[selected.clone()].to_string()
        };
        self.formatting = true;
        cx.spawn_in(window, async move |this, cx| {
            // The formatter trims a fragment's outer whitespace; a selection keeps its own, so
            // a line break after a `--` comment, say, still ends the comment.
            let formatted = cx
                .background_spawn(async move {
                    let core = part.trim();
                    let lead = &part[..part.len() - part.trim_start().len()];
                    let trail = &part[part.trim_end().len()..];
                    dbide_engine::format::format(core, engine)
                        .map(|text| format!("{lead}{text}{trail}"))
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.formatting = false;
                let editor = this.editor.read(cx);
                if editor.value() != whole || editor.selected_range() != selected {
                    this.say(
                        "The text changed while it was formatting, so it was left as it is. Format it again.",
                        Tone::Plain,
                        cx,
                    );
                    return;
                }
                match formatted {
                    Ok(text) => this.editor.update(cx, |editor, cx| {
                        if selected.is_empty() {
                            editor.replace_all(text, window, cx);
                        } else {
                            // Only the formatted range changes, not other cursors' selections.
                            editor.set_selected_range(selected, cx);
                            editor.replace(text, window, cx);
                        }
                    }),
                    Err(error) => this.say(format!("Couldn't format: {error}."), Tone::Error, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn state(&self, cx: &gpui::App) -> TabState {
        TabState {
            title: self.title.clone(),
            connection: self
                .target
                .as_ref()
                .map(|t| t.saved.connection.name.clone()),
            database: self.database.clone(),
            colour: crate::palette::name(self.target.as_ref().and_then(|t| t.colour.as_deref())),
            status: self.status,
            actual_plan: self.include_actual_plan,
            results_to_file: self.results_to_file,
            rows_written: self.file_progress().map(|(_, rows)| rows),
            cursor_line: self.editor.read(cx).cursor_position().line + 1,
            search: Some(self.editor.read(cx).search_session())
                .filter(|search| search.open)
                .map(|search| SearchState {
                    replace: search.replace_mode,
                    query: search.query.clone(),
                    matches: search.matcher.label(),
                }),
            file: self.file_path().map(std::path::Path::to_path_buf),
            modified: self.is_modified(cx),
            text: self.editor.read(cx).value().to_string(),
        }
    }

    pub fn dialog_state(&self, cx: &App) -> Option<DialogState> {
        self.dialog.as_ref().map(|dialog| dialog.state(cx))
    }

    /// The tab's label in the tab list: its file's name or title, with a dot while the text
    /// differs from its file, then its connection and database.
    pub fn label(&self, cx: &App) -> SharedString {
        let name = match self.is_modified(cx) {
            true => format!("● {}", self.name()),
            false => self.name().to_string(),
        };
        match (&self.target, &self.database) {
            (Some(target), Some(database)) => {
                format!("{name} · {} · {database}", target.saved.connection.name).into()
            }
            (Some(target), None) => format!("{name} · {}", target.saved.connection.name).into(),
            (None, _) => format!("{name} · not connected").into(),
        }
    }

    pub fn colour(&self) -> Option<&str> {
        self.target.as_ref().and_then(|t| t.colour.as_deref())
    }

    pub fn messages(&self) -> &Messages {
        &self.messages
    }

    pub fn results(&self) -> &Entity<Results> {
        &self.results
    }

    /// Adds one of the app's own notices to the tab's messages strip.
    pub fn note(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.say(text, Tone::Plain, cx);
    }

    fn say(&mut self, text: impl Into<SharedString>, tone: Tone, cx: &mut Context<Self>) {
        self.messages.push(Message {
            text: text.into(),
            tone,
            line: None,
        });
        cx.notify();
    }

    /// Up and Down in the strip: chooses the previous or next error that names a line.
    pub fn step_error(&mut self, forward: bool, cx: &mut Context<Self>) {
        self.messages.step(forward);
        cx.notify();
    }

    /// Enter in the strip: moves the caret to the chosen error's line.
    pub fn go_to_chosen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(line) = self.messages.chosen_line() {
            self.jump_to(line, window, cx);
        }
    }

    /// A click on message `ix`: chooses it and moves the caret to its line, if it has one.
    pub fn go_to_message(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(line) = self.messages.choose(ix) {
            self.jump_to(line, window, cx);
        }
    }

    /// Says why the session can't take new work, or returns it.
    fn take_session(&mut self, cx: &mut Context<Self>) -> Option<Box<dyn Session>> {
        let why = match self.status {
            Status::Ready => return self.session.take(),
            Status::Unconnected if self.target.is_some() => {
                "This tab connects when you run it (F5)."
            }
            Status::Unconnected => {
                "This tab has no connection. Select a connection in the explorer (F8) and press Ctrl+N."
            }
            Status::Connecting => "This tab is still connecting.",
            Status::Running => {
                "This tab is already running a query. Cancel it with Stop or Alt+Break."
            }
            Status::Switching => "This tab is switching databases.",
            Status::Listing => "This tab is loading its database list.",
            Status::Failed => {
                "This tab isn't connected. Open a new tab on the connection with Ctrl+N."
            }
        };
        self.say(why, Tone::Plain, cx);
        None
    }

    /// Takes the session back after work, reading its current database.
    fn give_back(&mut self, session: Box<dyn Session>) {
        self.database = Some(session.current_database().to_owned());
        self.session = Some(session);
        self.status = Status::Ready;
    }

    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = &self.target else { return };
        let name = target.saved.connection.name.clone();
        let connecting = self.runtime.connect(
            target.saved.connection.clone(),
            target.password.as_ref().map(|p| p.expose().to_owned()),
        );
        self.status = Status::Connecting;
        self.say(
            format!("Connecting {} to {name}…", self.title),
            Tone::Plain,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let outcome = connecting.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match outcome {
                    Ok(session) => {
                        this.give_back(session);
                        let database = this.database.clone().unwrap_or_default();
                        this.say(
                            format!("{} connected to {name}, database {database}.", this.title),
                            Tone::Plain,
                            cx,
                        );
                        if std::mem::take(&mut this.run_when_connected) {
                            this.run(window, cx);
                        }
                    }
                    Err(error) => {
                        this.run_when_connected = false;
                        this.status = Status::Failed;
                        this.say(
                            format!("Couldn't connect to {name}. {}", error.message),
                            Tone::Error,
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The selection, or the whole tab, with the editor line it starts on (from zero); `None`,
    /// saying why, when the session is busy or there is nothing to run.
    fn batch(&mut self, cx: &mut Context<Self>) -> Option<(String, u32)> {
        if self.status != Status::Ready {
            self.take_session(cx);
            return None;
        }
        let editor = self.editor.read(cx);
        let text = editor.value();
        let selected = editor.selected_range();
        let (batch, first_line) = if selected.is_empty() {
            (text.to_string(), 0)
        } else {
            let before = &text[..selected.start];
            (
                text[selected].to_string(),
                before.matches('\n').count() as u32,
            )
        };
        if batch.trim().is_empty() {
            self.say("There's nothing to run.", Tone::Plain, cx);
            return None;
        }
        Some((batch, first_line))
    }

    /// What F5 asks of the server: the results, or with Ctrl+M on, the actual plan too.
    pub fn run_job(&self) -> Job {
        if self.include_actual_plan {
            Job::Plan(PlanMode::Actual)
        } else {
            Job::Run
        }
    }

    /// Ctrl+M: turns returning the actual plan with each run on or off. A plan shows in the
    /// results pane, so it turns results to file off.
    pub fn toggle_actual_plan(&mut self, cx: &mut Context<Self>) {
        self.include_actual_plan = !self.include_actual_plan;
        let text = if self.include_actual_plan {
            self.results_to_file = false;
            "Include actual plan is on: F5 runs the batch and shows each statement's plan."
        } else {
            "Include actual plan is off."
        };
        self.say(text, Tone::Plain, cx);
    }

    /// Ctrl+Shift+F: turns sending each run's rows to a file on or off. A run to file shows
    /// nothing in the results pane, so it turns the actual plan off.
    pub fn toggle_results_to_file(&mut self, cx: &mut Context<Self>) {
        self.results_to_file = !self.results_to_file;
        let text = if self.results_to_file {
            self.include_actual_plan = false;
            "Results to file is on: F5 asks for a .csv or .xlsx file and writes the rows there."
        } else {
            "Results to file is off."
        };
        self.say(text, Tone::Plain, cx);
    }

    /// Starts F5's run of `batch`, once any danger check has passed: to the grid, or with
    /// results to file on, into the file the user names.
    pub fn go(
        &mut self,
        batch: String,
        first_line: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.results_to_file {
            self.start_run(batch, first_line, self.run_job(), window, cx);
            return;
        }
        let asked = crate::file_prompt::save_path(cx, crate::export_file::SUGGESTED_NAME);
        self.choosing_file = true;
        cx.spawn_in(window, async move |this, cx| {
            let path = asked.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.choosing_file = false;
                if let Some(path) = path {
                    match crate::export_file::run_format(&path) {
                        Ok(format) => {
                            let job = Job::ToFile {
                                path,
                                format,
                                rows_written: Default::default(),
                            };
                            this.start_run(batch, first_line, job, window, cx);
                        }
                        Err(reason) => this.say(reason, Tone::Error, cx),
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Ctrl+L: shows the estimated plan of the selection, or the whole tab, without running
    /// it, so it needs no danger check.
    pub fn estimated_plan(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((batch, first_line)) = self.batch(cx) {
            self.start_run(
                batch,
                first_line,
                Job::Plan(PlanMode::Estimated),
                window,
                cx,
            );
        }
    }

    /// Shows a plan opened from a file in place of the last run's results. Returns false,
    /// saying why, while a run is filling the results.
    pub fn show_plan(&mut self, plan: Plan, cx: &mut Context<Self>) -> bool {
        if self.status == Status::Running {
            self.say(
                "This tab's run is still filling its results. Open the plan when it ends, or in another tab.",
                Tone::Plain,
                cx,
            );
            return false;
        }
        self.results.update(cx, |results, cx| {
            results.clear(cx);
            results.add_plan(plan, true, cx);
        });
        true
    }

    /// F5: runs the selection, or the whole tab, after the danger check's confirmation when
    /// the batch has a dangerous statement and the connection asks for one. With Ctrl+M on,
    /// the run returns its actual plan too.
    pub fn run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_asleep() {
            self.wake(cx);
            return;
        }
        let Some((batch, first_line)) = self.batch(cx) else {
            return;
        };
        let Some(target) = &self.target else { return };
        if target.danger_check {
            let check = danger::check(&batch, target.saved.connection.engine);
            if !check.statements.is_empty() {
                self.ask_before_running(batch, first_line, check, window, cx);
                return;
            }
            if !check.fully_checked {
                self.say(
                    "The danger check couldn't parse this batch, so it was checked by keywords only.",
                    Tone::Warning,
                    cx,
                );
            }
        }
        self.go(batch, first_line, window, cx);
    }

    fn ask_before_running(
        &mut self,
        batch: String,
        first_line: u32,
        check: DangerCheck,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        dialogs::ask_before_running(self, batch, first_line, check, window, cx);
    }

    /// Sends `batch` to the server for `job`. `first_line` is the editor line the batch
    /// starts on, counted from zero, so the run's line numbers map back to the editor.
    fn start_run(
        &mut self,
        batch: String,
        first_line: u32,
        job: Job,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.take_session(cx) else {
            return;
        };
        self.status = Status::Running;
        self.cancel = None;
        self.cancel_pressed = false;
        self.run_cancelled = false;
        self.runs += 1;
        self.start_entry(&batch, &job);
        self.job = job.clone();
        let run = self.runs;
        self.results.update(cx, |results, cx| results.clear(cx));
        let mut updates = self.runtime.run(session, batch, job);
        // Row counts grow on the runtime without an update, in the grids or, for a run to file,
        // the rows written; redraw them while the run lasts.
        let results = self.results.downgrade();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(ROW_COUNT_REFRESH).await;
                let running = this.update(cx, |this, cx| {
                    let running = this.runs == run && this.status == Status::Running;
                    if running && matches!(this.job, Job::ToFile { .. }) {
                        cx.notify();
                    }
                    running
                });
                if !matches!(running, Ok(true))
                    || results.update(cx, Results::show_arrived_rows).is_err()
                {
                    return;
                }
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            while let Some(update) = updates.next().await {
                let applied = this.update_in(cx, |this, window, cx| {
                    this.apply(update, first_line, window, cx);
                    cx.notify();
                });
                if applied.is_err() {
                    return;
                }
            }
            // The channel closed without handing the session back: the engine stopped.
            let _ = this.update(cx, |this, cx| {
                if this.status == Status::Running {
                    this.status = Status::Failed;
                    this.cancel = None;
                    this.say("The engine stopped during the run.", Tone::Error, cx);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn apply(
        &mut self,
        update: RunUpdate,
        first_line: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The engine counts lines from one within the batch; the editor from zero.
        let editor_line =
            |line: Option<std::num::NonZeroU64>| line.map(|l| first_line + l.get() as u32 - 1);
        match &update {
            RunUpdate::Event(event) => self.note_event(event),
            RunUpdate::Written(Err(error)) => self.note_failure(error.to_string()),
            RunUpdate::Plan(Err(error)) => self.note_failure(error.to_string()),
            _ => {}
        }
        let message = match update {
            RunUpdate::Started(handle) => {
                if self.cancel_pressed {
                    handle.cancel();
                }
                self.cancel = Some(handle);
                return;
            }
            RunUpdate::ResultSet(reader) => {
                let engine = self.engine();
                self.results
                    .update(cx, |results, cx| results.add(reader, engine, window, cx));
                return;
            }
            RunUpdate::Written(Ok(files)) => {
                for file in &files {
                    self.say(crate::export_file::written(file), Tone::Plain, cx);
                }
                return;
            }
            RunUpdate::Written(Err(error)) => Message {
                text: format!("Couldn't write the results to a file: {error}").into(),
                tone: Tone::Error,
                line: None,
            },
            RunUpdate::Plan(Ok(plan)) => {
                // `.sqlplan` files hold SQL Server's showplan only.
                let savable = self.engine() == Some(Engine::SqlServer);
                self.results
                    .update(cx, |results, cx| results.add_plan(plan, savable, cx));
                return;
            }
            RunUpdate::Plan(Err(error)) => Message {
                text: format!("Couldn't read the plan: {error}").into(),
                tone: Tone::Error,
                line: None,
            },
            RunUpdate::Finished { session, results } => {
                self.cancel = None;
                self.give_back(session);
                self.finish_entry(cx);
                self.results.update(cx, |pane, cx| pane.keep(results, cx));
                // PostgreSQL's and MySQL's actual plans run each statement in a transaction
                // the engine rolls back (docs/spec.md, "Plans"); SQL Server's keep their
                // changes, as in SSMS.
                let rolled_back = match self.engine() {
                    Some(Engine::PostgreSql) => Some("PostgreSQL"),
                    Some(Engine::MySql) => Some("MySQL"),
                    Some(Engine::SqlServer) | None => None,
                };
                if matches!(self.job, Job::Plan(PlanMode::Actual))
                    && let Some(server) = rolled_back
                {
                    self.say(
                        format!(
                            "{server} ran the statements for their actual plans, then rolled back their changes."
                        ),
                        Tone::Plain,
                        cx,
                    );
                }
                return;
            }
            RunUpdate::Event(event) => match event {
                Event::Message {
                    text,
                    severity,
                    line,
                } => Message {
                    text: text.into(),
                    tone: if severity == Severity::Warning {
                        Tone::Warning
                    } else {
                        Tone::Plain
                    },
                    line: editor_line(line),
                },
                Event::RowCount(rows) => Message::plain(match rows {
                    1 => "(1 row affected)".to_owned(),
                    rows => format!("({rows} rows affected)"),
                }),
                Event::Error { text, line, code } => {
                    if code.is_none() && text == CANCELLED {
                        self.run_cancelled = true;
                    }
                    let line = editor_line(line);
                    let text = match line {
                        Some(line) => format!("Line {}: {text}", line + 1),
                        None => text,
                    };
                    Message {
                        text: text.into(),
                        tone: Tone::Error,
                        line,
                    }
                }
                Event::Done { elapsed } if self.run_cancelled => {
                    Message::plain(format!("Cancelled after {:.2} s.", elapsed.as_secs_f64()))
                }
                Event::Done { elapsed } => {
                    Message::plain(format!("Finished in {:.2} s.", elapsed.as_secs_f64()))
                }
                // The runtime turns these into result sets and plans.
                Event::ResultSetStarted { .. } | Event::Rows(_) | Event::Plan { .. } => return,
            },
        };
        self.messages.push(message);
    }

    /// The engine of the tab's connection.
    fn engine(&self) -> Option<Engine> {
        self.target.as_ref().map(|t| t.saved.connection.engine)
    }

    /// True once the runtime has started the current run, so a cancel reaches the server.
    pub fn has_started_run(&self) -> bool {
        self.cancel.is_some()
    }

    /// While a run to file is under way: the file it was asked to write and the rows written
    /// so far, across its files.
    pub fn file_progress(&self) -> Option<(&std::path::Path, u64)> {
        match &self.job {
            Job::ToFile {
                path, rows_written, ..
            } if self.status == Status::Running => Some((
                path,
                rows_written.load(std::sync::atomic::Ordering::Relaxed),
            )),
            _ => None,
        }
    }

    /// True while a run is under way, so Stop can cancel it.
    pub fn is_running(&self) -> bool {
        self.status == Status::Running
    }

    /// Stop or Alt+Break: cancels the run; rows and messages already received stay.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.status != Status::Running {
            self.say("Nothing is running in this tab.", Tone::Plain, cx);
            return;
        }
        self.cancel_pressed = true;
        if let Some(handle) = &self.cancel {
            handle.cancel();
        }
        self.say("Cancelling…", Tone::Plain, cx);
    }

    /// Ctrl+U: loads the server's databases and opens the picker.
    pub fn pick_database(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(session), Some(target)) = (self.take_session(cx), &self.target) else {
            return;
        };
        let engine = target.saved.connection.engine;
        self.status = Status::Listing;
        let listing = self.runtime.databases(session, engine);
        cx.spawn_in(window, async move |this, cx| {
            let (session, listed) = listing.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.give_back(session);
                match listed {
                    Ok(_) if !this.shown => this.say(
                        "You left this tab before its database list loaded. Press Ctrl+U to pick again.",
                        Tone::Plain,
                        cx,
                    ),
                    Ok(databases) => dialogs::pick_database(this, databases, window, cx),
                    Err(error) => this.say(
                        format!("Couldn't list the databases: {error}"),
                        Tone::Error,
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Switches the tab to `database`, keeping the old one if the server refuses.
    fn switch_database(&mut self, database: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.take_session(cx) else {
            return;
        };
        let password = self
            .target
            .as_ref()
            .and_then(|t| t.password.as_ref())
            .map(|p| p.expose().to_owned());
        self.status = Status::Switching;
        let switching = self
            .runtime
            .switch_database(session, database.clone(), password);
        cx.spawn_in(window, async move |this, cx| {
            let (session, switched) = switching.await;
            let _ = this.update(cx, |this, cx| {
                this.give_back(session);
                match switched {
                    Ok(()) => this.say(format!("Switched to {database}."), Tone::Plain, cx),
                    Err(error) => this.say(error, Tone::Error, cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Moves the caret to `line` (from zero) and gives the editor focus.
    pub fn jump_to(&mut self, line: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |editor, cx| {
            editor.set_cursor_position(Position::new(line, 0), window, cx);
            editor.focus(window, cx);
        });
    }

    /// Records whether the tab is on screen; the workbench calls it as tabs change.
    pub fn set_shown(&mut self, shown: bool) {
        self.shown = shown;
    }

    /// Gives the editor keyboard focus.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor
            .update(cx, |editor, cx| editor.focus(window, cx));
    }
}
