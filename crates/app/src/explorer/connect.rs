//! Connecting and disconnecting the explorer's connections: Enter connects with the saved
//! password, Windows sign-in or a typed one, and the connection's sessions live until it is
//! disconnected, edited or deleted. Ctrl+N signs in the same way and hands the connection to
//! the workbench for a query tab, which opens its own session.

use std::cell::Cell;

use dbide_engine::connections::{SavedConnection, SignInMethod};
use gpui::{
    App, AppContext as _, Context, Focusable as _, ParentElement as _, SharedString, Window, px,
};
use gpui_component::WindowExt as _;
use gpui_component::dialog::DialogEntrance;
use gpui_component::input::{Input, InputState};

use super::{Explorer, ExplorerEvent, OpenDialog, Server, Status, closer, footer};
use crate::query_tab::{HeldPassword, QueryTarget};

/// What to do once a connection's password is known.
#[derive(Clone, Copy)]
enum AfterSignIn {
    /// Connect the explorer's own session.
    Connect,
    /// Hand the connection to the workbench for a new query tab.
    OpenQuery,
}

impl Explorer {
    /// Drops the connection's sessions, what its tree loaded, and any connect still under
    /// way, so the next connect uses its current details. Returns true if it was connected.
    pub(super) fn forget_session(&mut self, id: &str) -> bool {
        self.connecting.remove(id);
        match self.servers.remove(id) {
            Some(server) => {
                server.close(&self.runtime);
                true
            }
            None => false,
        }
    }

    /// Enter: opens or closes a folder or object row, or connects a connection.
    pub(super) fn activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected(cx) else { return };
        if self.object_rows.contains_key(&id) {
            self.toggle(&id, cx);
            return;
        }
        if self.is_folder(&id) {
            if !self.expanded.remove(&id) {
                self.expanded.insert(id.clone());
            }
            self.rebuild(Some(&id), cx);
            return;
        }
        let Some(saved) = self.connection(&id).cloned() else {
            return;
        };
        match self.status(&id) {
            Status::Connected => {
                self.message(
                    format!("Already connected to {}.", saved.connection.name),
                    cx,
                );
            }
            Status::Connecting => {}
            Status::Disconnected => self.sign_in(saved, AfterSignIn::Connect, window, cx),
        }
    }

    /// Ctrl+N with a connection selected: signs in as Enter does, then asks the workbench
    /// for a query tab on it. With a database or object selected, the tab opens on that
    /// database. Returns false when neither is selected.
    pub(crate) fn open_query(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.query_on_selected_database(cx) {
            return true;
        }
        let Some(saved) = self
            .selected(cx)
            .and_then(|id| self.connection(&id).cloned())
        else {
            return false;
        };
        self.sign_in(saved, AfterSignIn::OpenQuery, window, cx);
        true
    }

    /// Finds the connection's password (saved, asked for, or none for Windows sign-in),
    /// then does `after`.
    fn sign_in(
        &mut self,
        saved: SavedConnection,
        after: AfterSignIn,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match saved.connection.sign_in {
            SignInMethod::Windows => self.signed_in(&saved, None, after, window, cx),
            SignInMethod::Password => match self.credentials.read(&saved) {
                Ok(Some(secret)) => {
                    let password = secret.expose().to_owned();
                    self.signed_in(&saved, Some(password), after, window, cx);
                }
                Ok(None) => self.ask_password(saved, after, window, cx),
                Err(error) => {
                    self.message(format!("Can't read the saved password: {error}."), cx);
                }
            },
            SignInMethod::AskEachTime => self.ask_password(saved, after, window, cx),
        }
    }

    fn signed_in(
        &mut self,
        saved: &SavedConnection,
        password: Option<String>,
        after: AfterSignIn,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match after {
            AfterSignIn::Connect => self.connect(saved, password, window, cx),
            AfterSignIn::OpenQuery => {
                self.dialog = None;
                let settings = self.saved.effective_settings(&saved.id).ok();
                cx.emit(ExplorerEvent::OpenQuery(Box::new(QueryTarget {
                    saved: saved.clone(),
                    colour: settings.as_ref().and_then(|s| s.colour).map(str::to_owned),
                    danger_check: settings.is_none_or(|s| s.danger_check),
                    password: password.map(HeldPassword::new),
                })));
            }
        }
    }

    fn ask_password(
        &mut self,
        saved: SavedConnection,
        after: AfterSignIn,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| InputState::new(window, cx).masked(true));
        let connection: SharedString = saved.connection.name.clone().into();
        self.dialog = Some(OpenDialog::Password {
            connection: connection.clone(),
            input: input.clone(),
        });
        let explorer = cx.entity();
        let first_open = Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if first_open.replace(false) {
                input.focus_handle(cx).focus(window, cx);
            }
            let go = {
                let (explorer, input, saved) = (explorer.clone(), input.clone(), saved.clone());
                move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                    let password = input.read(cx).value().to_string();
                    explorer.update(cx, |this, cx| {
                        this.signed_in(&saved, Some(password), after, window, cx)
                    });
                    true
                }
            };
            dialog
                .title(format!("Password for {connection}"))
                .entrance(DialogEntrance::None)
                .w(px(420.))
                .child(Input::new(&input))
                .footer(footer("Connect (Enter)"))
                .on_ok(go)
                .on_close(closer(&explorer))
        });
    }

    fn connect(
        &mut self,
        saved: &SavedConnection,
        password: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = saved.id.clone();
        let name = saved.connection.name.clone();
        let attempt = self.next_attempt;
        self.next_attempt += 1;
        self.connecting.insert(id.clone(), attempt);
        self.message(format!("Connecting to {name}…"), cx);
        cx.notify();
        let connecting = self
            .runtime
            .connect(saved.connection.clone(), password.clone());
        let saved = saved.clone();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = connecting.await;
            let _ = this.update(cx, |this, cx| {
                if this.connecting.get(&id) != Some(&attempt) {
                    // The connection was edited or deleted while connecting: its details
                    // are out of date, so a session from them is closed, not kept.
                    if let Ok(session) = outcome {
                        this.runtime.close(session);
                    }
                    return;
                }
                this.connecting.remove(&id);
                match outcome {
                    Ok(session) => {
                        let server = Server::new(attempt, saved, password, session);
                        this.servers.insert(id, server);
                        this.message(format!("Connected to {name}."), cx);
                        this.rebuild(None, cx);
                    }
                    Err(error) => {
                        this.message(format!("Couldn't connect to {name}. {}", error.message), cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn disconnect(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected(cx) else { return };
        let name = self
            .describe(&id)
            .map(|(_, name, _)| name)
            .unwrap_or_default();
        let was_connecting = self.connecting.contains_key(&id);
        if self.forget_session(&id) {
            self.message(format!("Disconnected from {name}."), cx);
        } else if was_connecting {
            self.message(format!("Stopped connecting to {name}."), cx);
        }
        self.rebuild(None, cx);
    }
}
