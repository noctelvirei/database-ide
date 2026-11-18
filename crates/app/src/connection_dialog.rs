//! The connection dialog: every field of a saved connection, its password, and Test.
//!
//! The form reads and writes `dbide_engine::connections::Connection`; the explorer opens it
//! in a gpui-component dialog and saves what `connection` returns. Test connects with the
//! typed details on the app's runtime and shows the engine's message unchanged.

use std::rc::Rc;

use dbide_engine::connections::credentials::Credentials;
use dbide_engine::connections::{Connection, SavedConnection, SignInMethod, TlsMode};
use dbide_engine::danger::Engine;
use gpui::{
    AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Window, div, prelude::*,
};
use gpui_component::ActiveTheme as _;
use gpui_component::button::Button;
use gpui_component::checkbox::Checkbox;
use gpui_component::form::{field, v_form};
use gpui_component::input::{Input, InputState};
use gpui_component::select::{Select, SelectEvent};
use serde::Serialize;

use crate::choice::{self, Choice, choice, label};
use crate::keymap::TestConnection;
use crate::palette;
use crate::runtime::Runtime;

const ENGINES: [(&str, Engine); 3] = [
    ("SQL Server", Engine::SqlServer),
    ("PostgreSQL", Engine::PostgreSql),
    ("MySQL", Engine::MySql),
];

const SIGN_INS: [(&str, SignInMethod); 3] = [
    ("Saved password", SignInMethod::Password),
    ("Windows account", SignInMethod::Windows),
    ("Ask for the password", SignInMethod::AskEachTime),
];

const TLS_MODES: [(&str, TlsMode); 3] = [
    ("Required", TlsMode::Mandatory),
    ("If the server offers it", TlsMode::Optional),
    ("Off", TlsMode::Off),
];

/// The danger check choices; the first inherits from the folder.
pub const DANGER_CHECKS: [(&str, Option<bool>); 3] = [
    ("Inherit from folder", None),
    ("On", Some(true)),
    ("Off", Some(false)),
];

/// Where Test stands, shown under the form.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "message", rename_all = "snake_case")]
pub enum TestStatus {
    NotRun,
    Testing,
    Connected,
    Failed(SharedString),
}

/// What the dialog shows, recorded by the UI script. The password is reported only as
/// whether one is typed.
#[derive(Debug, Serialize)]
pub struct FormState {
    pub name: String,
    pub engine: &'static str,
    pub host: String,
    pub port: String,
    pub instance: String,
    pub sign_in: &'static str,
    pub user: String,
    pub password_typed: bool,
    pub database: String,
    pub colour: &'static str,
    pub danger_check: &'static str,
    pub tls: &'static str,
    pub trust_server_certificate: bool,
    /// Why Save refused the details, if it did.
    pub error: Option<SharedString>,
    pub test: TestStatus,
}

/// What Save hands the explorer: the details and the password to store, if any.
pub struct FormOutput {
    pub connection: Connection,
    /// A typed password, for saved-password sign-in only.
    pub password: Option<String>,
}

pub struct ConnectionForm {
    focus: FocusHandle,
    name: Entity<InputState>,
    engine: Entity<Choice>,
    host: Entity<InputState>,
    port: Entity<InputState>,
    instance: Entity<InputState>,
    sign_in: Entity<Choice>,
    user: Entity<InputState>,
    password: Entity<InputState>,
    database: Entity<InputState>,
    colour: Entity<Choice>,
    danger_check: Entity<Choice>,
    tls: Entity<Choice>,
    trust_server_certificate: bool,
    application_name: Entity<InputState>,
    connect_timeout: Entity<InputState>,
    query_timeout: Entity<InputState>,
    /// The connection being edited, for its stored password; `None` for a new one.
    editing: Option<SavedConnection>,
    credentials: Rc<Credentials>,
    runtime: Runtime,
    error: Option<SharedString>,
    test: TestStatus,
}

impl ConnectionForm {
    /// A form filled from `editing`, or with a new connection's defaults.
    pub fn new(
        editing: Option<SavedConnection>,
        credentials: Rc<Credentials>,
        runtime: Runtime,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let details = editing.as_ref().map_or_else(
            || Connection::new(Engine::SqlServer, ""),
            |saved| saved.connection.clone(),
        );
        let mut text = |value: &str, placeholder: &str| {
            let value = value.to_owned();
            let placeholder = placeholder.to_owned();
            cx.new(|cx| {
                let mut state = InputState::new(window, cx).placeholder(placeholder);
                state.set_value(value, window, cx);
                state
            })
        };
        let name = text(&details.name, "The host, if left blank");
        let host = text(&details.host, "Host name or IP address");
        let port = text(
            &details.port.map(|p| p.to_string()).unwrap_or_default(),
            "Default",
        );
        let instance = text(
            details.instance.as_deref().unwrap_or(""),
            "Default instance",
        );
        let user = text(details.user.as_deref().unwrap_or(""), "");
        let database = text(
            details.database.as_deref().unwrap_or(""),
            "The login's default",
        );
        let application_name = text(&details.application_name, "");
        let connect_timeout = text(&details.connect_timeout.to_string(), "0 waits forever");
        let query_timeout = text(&details.query_timeout.to_string(), "0 waits forever");
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));

        let engine = choice(&ENGINES, details.engine, window, cx);
        let sign_in = choice(&SIGN_INS, details.sign_in, window, cx);
        let tls = choice(&TLS_MODES, details.tls, window, cx);
        let danger_check = choice(&DANGER_CHECKS, details.danger_check, window, cx);
        let colour = choice(
            &palette::CHOICES,
            palette::known(details.colour.as_deref()),
            window,
            cx,
        );
        // Fields appear and disappear with the engine and sign-in choices.
        for select in [&engine, &sign_in] {
            cx.subscribe(select, |_, _, _: &SelectEvent<Vec<&'static str>>, cx| {
                cx.notify()
            })
            .detach();
        }

        Self {
            focus: cx.focus_handle(),
            name,
            engine,
            host,
            port,
            instance,
            sign_in,
            user,
            password,
            database,
            colour,
            danger_check,
            tls,
            trust_server_certificate: details.trust_server_certificate,
            application_name,
            connect_timeout,
            query_timeout,
            editing,
            credentials,
            runtime,
            error: None,
            test: TestStatus::NotRun,
        }
    }

    /// The field the dialog focuses when it opens.
    pub fn first_field(&self, cx: &gpui::App) -> FocusHandle {
        self.name.focus_handle(cx)
    }

    /// Checks the fields and returns the details to save, or shows why they can't be saved
    /// and returns `None`.
    pub fn output(&mut self, cx: &mut Context<Self>) -> Option<FormOutput> {
        let result = self.read(cx);
        self.error = result.as_ref().err().cloned();
        cx.notify();
        let connection = result.ok()?;
        let password = self.password.read(cx).value().to_string();
        let password = (connection.sign_in == SignInMethod::Password && !password.is_empty())
            .then_some(password);
        Some(FormOutput {
            connection,
            password,
        })
    }

    pub fn state(&self, cx: &gpui::App) -> FormState {
        let value = |input: &Entity<InputState>| input.read(cx).value().to_string();
        FormState {
            name: value(&self.name),
            engine: label(&self.engine, cx),
            host: value(&self.host),
            port: value(&self.port),
            instance: value(&self.instance),
            sign_in: label(&self.sign_in, cx),
            user: value(&self.user),
            password_typed: !self.password.read(cx).value().is_empty(),
            database: value(&self.database),
            colour: label(&self.colour, cx),
            danger_check: label(&self.danger_check, cx),
            tls: label(&self.tls, cx),
            trust_server_certificate: self.trust_server_certificate,
            error: self.error.clone(),
            test: self.test.clone(),
        }
    }

    /// Reads every field into a connection, with a sentence for the first one that is wrong.
    fn read(&self, cx: &gpui::App) -> Result<Connection, SharedString> {
        let value = |input: &Entity<InputState>| input.read(cx).value().trim().to_string();
        let optional = |input: &Entity<InputState>| Some(value(input)).filter(|v| !v.is_empty());
        let seconds = |input: &Entity<InputState>, label: &str| {
            let text = value(input);
            if text.is_empty() {
                return Ok(0);
            }
            text.parse::<u32>().map_err(|_| {
                SharedString::from(format!("{label} must be a whole number of seconds."))
            })
        };

        let host = value(&self.host);
        if host.is_empty() {
            return Err("Enter the server's host name or IP address.".into());
        }
        let engine = choice::value(&ENGINES, &self.engine, cx);
        let port = match optional(&self.port) {
            None => None,
            Some(text) => match text.parse::<u16>() {
                Ok(port) if port > 0 => Some(port),
                _ => return Err("Port must be a number from 1 to 65535.".into()),
            },
        };
        let instance = optional(&self.instance).filter(|_| engine == Engine::SqlServer);
        if port.is_some() && instance.is_some() {
            return Err("Give a port or an instance name, not both.".into());
        }
        let name = optional(&self.name).unwrap_or_else(|| host.clone());
        let mut connection = Connection::new(engine, host);
        connection.name = name;
        connection.port = port;
        connection.instance = instance;
        connection.sign_in = choice::value(&SIGN_INS, &self.sign_in, cx);
        connection.user = optional(&self.user);
        connection.database = optional(&self.database);
        connection.colour = choice::value(&palette::CHOICES, &self.colour, cx).map(str::to_owned);
        connection.danger_check = choice::value(&DANGER_CHECKS, &self.danger_check, cx);
        connection.tls = choice::value(&TLS_MODES, &self.tls, cx);
        connection.trust_server_certificate = self.trust_server_certificate;
        connection.application_name = value(&self.application_name);
        connection.connect_timeout = seconds(&self.connect_timeout, "Connect timeout")?;
        connection.query_timeout = seconds(&self.query_timeout, "Query timeout")?;
        Ok(connection)
    }

    /// Connects with the typed details, or the stored password when none is typed, and
    /// shows the outcome. The test connection is closed straight away.
    fn test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.test == TestStatus::Testing {
            return;
        }
        let connection = match self.read(cx) {
            Ok(connection) => connection,
            Err(error) => {
                self.test = TestStatus::Failed(error);
                cx.notify();
                return;
            }
        };
        let typed = self.password.read(cx).value().to_string();
        let password = match (connection.sign_in, typed.is_empty(), &self.editing) {
            (SignInMethod::Windows, _, _) => None,
            (_, false, _) => Some(typed),
            (SignInMethod::Password, true, Some(saved)) => match self.credentials.read(saved) {
                Ok(secret) => secret.map(|s| s.expose().to_owned()),
                Err(error) => {
                    self.test = TestStatus::Failed(
                        format!("Can't read the saved password: {error}.").into(),
                    );
                    cx.notify();
                    return;
                }
            },
            (_, true, _) => None,
        };
        self.test = TestStatus::Testing;
        cx.notify();
        let attempt = self.runtime.connect(connection, password);
        cx.spawn_in(window, async move |this, cx| {
            let outcome = attempt.await;
            let _ = this.update(cx, |this, cx| {
                this.test = match outcome {
                    Ok(session) => {
                        this.runtime.close(session);
                        TestStatus::Connected
                    }
                    Err(error) => TestStatus::Failed(error.message.into()),
                };
                cx.notify();
            });
        })
        .detach();
    }
}

impl Focusable for ConnectionForm {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ConnectionForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let engine = choice::value(&ENGINES, &self.engine, cx);
        let sign_in = choice::value(&SIGN_INS, &self.sign_in, cx);
        let default_port = match engine {
            Engine::SqlServer => "1433",
            Engine::PostgreSql => "5432",
            Engine::MySql => "3306",
        };
        let password_label = match (sign_in, &self.editing) {
            (SignInMethod::Password, Some(_)) => "Password (blank keeps the saved one)",
            (SignInMethod::Password, None) => "Password",
            _ => "Password, for Test only",
        };
        let user_shown = !(engine == Engine::SqlServer && sign_in == SignInMethod::Windows);

        let form = v_form()
            .child(field().label("Name").child(Input::new(&self.name)))
            .child(field().label("Engine").child(Select::new(&self.engine)))
            .child(
                field()
                    .label("Host")
                    .required(true)
                    .child(Input::new(&self.host)),
            )
            .child(
                field()
                    .label(format!("Port (default {default_port})"))
                    .child(Input::new(&self.port)),
            )
            .when(engine == Engine::SqlServer, |form| {
                form.child(field().label("Instance").child(Input::new(&self.instance)))
            })
            .child(field().label("Sign-in").child(Select::new(&self.sign_in)))
            .when(user_shown, |form| {
                form.child(field().label("User").child(Input::new(&self.user)))
            })
            .when(sign_in != SignInMethod::Windows, |form| {
                form.child(
                    field()
                        .label(password_label)
                        .child(Input::new(&self.password)),
                )
            })
            .child(field().label("Database").child(Input::new(&self.database)))
            .child(field().label("Colour").child(Select::new(&self.colour)))
            .child(
                field()
                    .label("Danger check")
                    .child(Select::new(&self.danger_check)),
            )
            .child(field().label("Encryption").child(Select::new(&self.tls)))
            .child(
                field().child(
                    Checkbox::new("trust-server-certificate")
                        .label("Trust server certificate")
                        .checked(self.trust_server_certificate)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.trust_server_certificate = *checked;
                            cx.notify();
                        })),
                ),
            )
            .child(
                field()
                    .label("Application name")
                    .child(Input::new(&self.application_name)),
            )
            .child(
                field()
                    .label("Connect timeout, seconds")
                    .child(Input::new(&self.connect_timeout)),
            )
            .child(
                field()
                    .label("Query timeout, seconds")
                    .child(Input::new(&self.query_timeout)),
            );

        let test_line = match &self.test {
            TestStatus::NotRun => None,
            TestStatus::Testing => Some(("Testing…".into(), theme.muted_foreground)),
            TestStatus::Connected => Some(("Connected.".into(), theme.success)),
            TestStatus::Failed(message) => Some((message.clone(), theme.danger)),
        };

        div()
            .id("connection-form")
            .key_context("ConnectionForm")
            .track_focus(&self.focus)
            .on_action(choice::confirm_dialog(self.focus.clone()))
            .on_action(cx.listener(|this, _: &TestConnection, window, cx| this.test(window, cx)))
            .flex()
            .flex_col()
            .gap_2()
            .max_h(gpui::px(560.))
            .overflow_y_scroll()
            .child(form)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .items_center()
                    .child(
                        Button::new("test")
                            .label("Test (Alt+T)")
                            .on_click(cx.listener(|this, _, window, cx| this.test(window, cx))),
                    )
                    .children(test_line.map(|(text, colour)| div().text_color(colour).child(text))),
            )
            .children(
                self.error
                    .clone()
                    .map(|error| div().text_color(theme.danger).child(error)),
            )
    }
}
