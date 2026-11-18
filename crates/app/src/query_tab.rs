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
use dbide_engine::danger::{self, DangerCheck};
use dbide_engine::session::{CancelHandle, Event, Session, Severity};
use futures::StreamExt as _;
use gpui::{App, AppContext as _, Context, Entity, EventEmitter, SharedString, Window};
use gpui_component::input::{EditorState, Position};
use serde::Serialize;

use crate::messages::{Jump, Message, Tone};
use crate::runtime::{RunUpdate, Runtime};

pub mod dialogs;

use dialogs::DialogState;

/// A password held in memory for the tab's reconnects; never shown or logged.
#[derive(Clone)]
pub struct HeldPassword(String);

impl HeldPassword {
    pub fn new(password: String) -> Self {
        Self(password)
    }

    fn expose(&self) -> &str {
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
    /// The editor caret's line, counted from one.
    pub cursor_line: u32,
}

/// Something for the messages strip.
pub enum QueryTabEvent {
    Message(Message),
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
    /// Cancel was pressed before the run had started; it is cancelled as it starts.
    cancel_requested: bool,
    dialog: Option<dialogs::OpenDialog>,
}

impl EventEmitter<QueryTabEvent> for QueryTab {}

impl QueryTab {
    /// A tab titled `title`, connecting at once when it has a target.
    pub fn new(
        title: SharedString,
        target: Option<QueryTarget>,
        runtime: Runtime,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = cx.new(|cx| EditorState::new(window, cx).language("sql"));
        let mut tab = Self {
            title,
            target,
            editor,
            runtime,
            status: Status::Unconnected,
            session: None,
            database: None,
            cancel: None,
            cancel_requested: false,
            dialog: None,
        };
        tab.connect(window, cx);
        tab
    }

    pub fn editor(&self) -> &Entity<EditorState> {
        &self.editor
    }

    pub fn target(&self) -> Option<&QueryTarget> {
        self.target.as_ref()
    }

    /// True while the session is away doing work: connecting, running, switching or listing.
    pub fn is_busy(&self) -> bool {
        matches!(
            self.status,
            Status::Connecting | Status::Running | Status::Switching | Status::Listing
        )
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
            cursor_line: self.editor.read(cx).cursor_position().line + 1,
        }
    }

    pub fn dialog_state(&self, cx: &App) -> Option<DialogState> {
        self.dialog.as_ref().map(|dialog| dialog.state(cx))
    }

    /// The tab's label in the tab list: its title, connection and database.
    pub fn label(&self) -> SharedString {
        match (&self.target, &self.database) {
            (Some(target), Some(database)) => format!(
                "{} · {} · {database}",
                self.title, target.saved.connection.name
            )
            .into(),
            (Some(target), None) => {
                format!("{} · {}", self.title, target.saved.connection.name).into()
            }
            (None, _) => format!("{} · not connected", self.title).into(),
        }
    }

    pub fn colour(&self) -> Option<&str> {
        self.target.as_ref().and_then(|t| t.colour.as_deref())
    }

    fn say(&self, text: impl Into<SharedString>, tone: Tone, cx: &mut Context<Self>) {
        cx.emit(QueryTabEvent::Message(Message {
            text: text.into(),
            tone,
            jump: None,
        }));
    }

    /// Says why the session can't take new work, or returns it.
    fn take_session(&mut self, cx: &mut Context<Self>) -> Option<Box<dyn Session>> {
        let why = match self.status {
            Status::Ready => return self.session.take(),
            Status::Unconnected => {
                "This tab has no connection. Select a connection in the explorer (F8) and press Ctrl+N."
            }
            Status::Connecting => "This tab is still connecting.",
            Status::Running => "This tab is already running a query. Cancel it with Alt+Break.",
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
            let _ = this.update(cx, |this, cx| {
                match outcome {
                    Ok(session) => {
                        this.give_back(session);
                        let database = this.database.clone().unwrap_or_default();
                        this.say(
                            format!("{} connected to {name}, database {database}.", this.title),
                            Tone::Plain,
                            cx,
                        );
                    }
                    Err(error) => {
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

    /// F5: runs the selection, or the whole tab, after the danger check's confirmation when
    /// the batch has a dangerous statement and the connection asks for one.
    pub fn run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.status != Status::Ready {
            self.take_session(cx);
            return;
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
            return;
        }
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
        self.start_run(batch, first_line, window, cx);
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

    /// Sends `batch` to the server. `first_line` is the editor line the batch starts on,
    /// counted from zero, so the run's line numbers map back to the editor.
    fn start_run(
        &mut self,
        batch: String,
        first_line: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.take_session(cx) else {
            return;
        };
        self.status = Status::Running;
        self.cancel = None;
        self.cancel_requested = false;
        let mut updates = self.runtime.run(session, batch);
        let tab = cx.entity_id();
        cx.spawn_in(window, async move |this, cx| {
            while let Some(update) = updates.next().await {
                let applied = this.update(cx, |this, cx| {
                    this.apply(update, tab, first_line, cx);
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
        tab: gpui::EntityId,
        first_line: u32,
        cx: &mut Context<Self>,
    ) {
        let jump = |line: Option<std::num::NonZeroU64>| {
            line.map(|line| Jump {
                tab,
                line: first_line + line.get() as u32 - 1,
            })
        };
        let message = match update {
            RunUpdate::Started(handle) => {
                if self.cancel_requested {
                    handle.cancel();
                }
                self.cancel = Some(handle);
                return;
            }
            RunUpdate::Finished(session) => {
                self.cancel = None;
                self.give_back(session);
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
                    jump: jump(line),
                },
                Event::RowCount(rows) => Message::plain(match rows {
                    1 => "(1 row affected)".to_owned(),
                    rows => format!("({rows} rows affected)"),
                }),
                Event::Error { text, line, .. } => {
                    let jump = jump(line);
                    let text = match jump {
                        Some(jump) => format!("Line {}: {text}", jump.line + 1),
                        None => text,
                    };
                    Message {
                        text: text.into(),
                        tone: Tone::Error,
                        jump,
                    }
                }
                Event::Done { elapsed } => {
                    Message::plain(format!("Finished in {:.2} s.", elapsed.as_secs_f64()))
                }
                // U4 shows result sets and plans.
                Event::ResultSetStarted { .. } | Event::Rows(_) | Event::Plan { .. } => return,
            },
        };
        cx.emit(QueryTabEvent::Message(message));
    }

    /// True once the runtime has started the current run, so a cancel reaches the server.
    pub fn has_started_run(&self) -> bool {
        self.cancel.is_some()
    }

    /// Alt+Break: cancels the run; rows and messages already received stay.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.status != Status::Running {
            self.say("Nothing is running in this tab.", Tone::Plain, cx);
            return;
        }
        match &self.cancel {
            Some(handle) => handle.cancel(),
            None => self.cancel_requested = true,
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

    /// Gives the editor keyboard focus.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor
            .update(cx, |editor, cx| editor.focus(window, cx));
    }
}
